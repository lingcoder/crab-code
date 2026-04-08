//! `SendUserFileTool` — send a file to the user.
//!
//! Packages a file from the workspace and presents it to the user for
//! download or preview. Useful for generated reports, images, exported
//! data, and build artifacts.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `SendUserFileTool`.
pub const SEND_USER_FILE_TOOL_NAME: &str = "SendUserFile";

// ── Input types ───────────────────────────────────────────────────────

/// Parsed input for the SendUserFile tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendUserFileInput {
    /// Absolute or workspace-relative path to the file to send.
    pub file_path: String,
    /// Optional human-readable description of what the file contains.
    #[serde(default)]
    pub description: Option<String>,
}

// ── Tool implementation ───────────────────────────────────────────────

/// Send a file to the user for download or preview.
///
/// Input schema:
/// ```json
/// {
///   "file_path": "<path to file>",
///   "description": "<optional description>"
/// }
/// ```
pub struct SendUserFileTool;

impl Tool for SendUserFileTool {
    fn name(&self) -> &str {
        SEND_USER_FILE_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Send a file to the user for download or preview. Provide the file path \
         and an optional description. The file must exist in the workspace."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to send to the user"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description of the file contents"
                }
            },
            "required": ["file_path"]
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
            let parsed: SendUserFileInput = serde_json::from_value(input)
                .map_err(|e| crab_common::Error::Tool(format!("Invalid input: {e}")))?;

            todo!(
                "SendUserFile::execute: read file '{}', package for user delivery",
                parsed.file_path
            )
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_metadata() {
        let tool = SendUserFileTool;
        assert_eq!(tool.name(), "SendUserFile");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_requires_file_path() {
        let schema = SendUserFileTool.input_schema();
        assert_eq!(schema["required"], json!(["file_path"]));
    }

    #[test]
    fn input_parse_with_description() {
        let input: SendUserFileInput = serde_json::from_value(json!({
            "file_path": "/tmp/report.pdf",
            "description": "Generated test report"
        }))
        .unwrap();
        assert_eq!(input.file_path, "/tmp/report.pdf");
        assert_eq!(input.description.as_deref(), Some("Generated test report"));
    }

    #[test]
    fn input_parse_without_description() {
        let input: SendUserFileInput = serde_json::from_value(json!({
            "file_path": "output.csv"
        }))
        .unwrap();
        assert_eq!(input.file_path, "output.csv");
        assert!(input.description.is_none());
    }
}
