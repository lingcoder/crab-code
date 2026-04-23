//! `WorkflowTool` — multi-step predefined workflows.
//!
//! Executes named workflows that combine multiple tool calls into a single
//! high-level operation. Workflows are defined declaratively and can accept
//! arguments to customize their behavior.
//!
//! Examples: "lint-and-fix", "test-and-commit", "review-pr".

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `WorkflowTool`.
pub const WORKFLOW_TOOL_NAME: &str = "Workflow";

// ── Input types ───────────────────────────────────────────────────────

/// Parsed input for the Workflow tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    /// Name of the workflow to execute.
    pub name: String,
    /// Optional arguments for the workflow.
    #[serde(default)]
    pub args: Option<Value>,
}

// ── Tool implementation ───────────────────────────────────────────────

/// Multi-step workflow executor.
///
/// Input schema:
/// ```json
/// {
///   "name": "<workflow name>",
///   "args": { ... }
/// }
/// ```
pub struct WorkflowTool;

impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        WORKFLOW_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Execute a predefined multi-step workflow by name. Workflows combine \
         multiple tool calls into a single high-level operation. Pass optional \
         arguments to customize behavior."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the workflow to execute"
                },
                "args": {
                    "type": "object",
                    "description": "Optional arguments for the workflow"
                }
            },
            "required": ["name"]
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        input["name"].as_str().map(|n| format!("Workflow ({n})"))
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let parsed: WorkflowInput = serde_json::from_value(input)
                .map_err(|e| crab_core::Error::Tool(format!("Invalid input: {e}")))?;

            Ok(ToolOutput::error(format!(
                "Workflow '{}' execution is not yet implemented. \
                 Workflow definitions and the step runner are planned \
                 for a future release.",
                parsed.name
            )))
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
        let tool = WorkflowTool;
        assert_eq!(tool.name(), "Workflow");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_name() {
        let schema = WorkflowTool.input_schema();
        assert_eq!(schema["required"], json!(["name"]));
        assert!(schema["properties"]["name"].is_object());
    }

    #[test]
    fn input_parse_with_args() {
        let input: WorkflowInput = serde_json::from_value(json!({
            "name": "lint-and-fix",
            "args": {"path": "src/"}
        }))
        .unwrap();
        assert_eq!(input.name, "lint-and-fix");
        assert!(input.args.is_some());
    }

    #[test]
    fn input_parse_without_args() {
        let input: WorkflowInput = serde_json::from_value(json!({
            "name": "test-all"
        }))
        .unwrap();
        assert_eq!(input.name, "test-all");
        assert!(input.args.is_none());
    }
}
